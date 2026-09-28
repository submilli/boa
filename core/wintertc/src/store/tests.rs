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
    let value=cx.eval(Source::from_bytes("var transferred=new ArrayBuffer(8);[transferred,transferred,new Uint8Array(transferred)]")).unwrap();
    let buffer = cx.eval(Source::from_bytes("transferred")).unwrap();
    let store = JsValueStore::try_from_js(&value, &mut cx, vec![buffer]).unwrap();
    let clone = store.try_into_js(&mut cx).unwrap().as_object().unwrap();
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
