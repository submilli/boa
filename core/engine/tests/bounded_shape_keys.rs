//! Shape key snapshots must admit their size before cloning.
// Integration tests share the engine dependency list while using only its API.
#![allow(unused_crate_dependencies)]

use boa_engine::{Context, Source};

#[test]
fn shape_key_admission_precedes_cloning_for_shared_and_unique_shapes() {
    let mut cx = Context::default();
    for source in [
        "({a:1,b:2,[Symbol('s')]:3})",
        "(()=>{const o={};for(let i=0;i<10000;i++)o['k'+i]=i;return o})()",
        "(()=>{const o={a:1,b:2,c:3};delete o.b;return o})()",
    ] {
        let value = cx.eval(Source::from_bytes(source)).unwrap();
        let object = value.as_object().unwrap();
        let borrowed = object.borrow();
        let expected = borrowed.shape().keys();
        assert!(borrowed.shape().keys_bounded(expected.len() - 1).is_none());
        assert_eq!(
            borrowed.shape().keys_bounded(expected.len()),
            Some(expected)
        );
    }
}
