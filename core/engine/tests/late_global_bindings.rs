//! Functions compiled by an earlier script must see later global declarations.
#![allow(unused_crate_dependencies)]
use boa_engine::{Context, Source};

#[test]
fn earlier_functions_resolve_later_global_lexicals() {
    let context = &mut Context::default();
    context
        .eval(Source::from_bytes(
            r#"
        globalThis.shadowed = 1;
        globalThis.readLate = new Function("return late");
        globalThis.writeLate = new Function("late = 43");
        globalThis.typeLate = new Function("return typeof late");
        globalThis.readShadowed = () => shadowed;
        globalThis.writeConst = new Function("constant = 3");
        globalThis.withLate = new Function("with ({late: 7}) { return late; }");
        globalThis.evalLate = new Function("eval(\"var late = 6\"); return late");
        globalThis.closed = (() => { let local = 8; return () => [local, late]; })();
        readShadowed(); // Warm the global-object inline cache.
    "#,
        ))
        .unwrap();
    assert!(context.eval(Source::from_bytes("readLate()")).is_err());
    context
        .eval(Source::from_bytes(
            r"
        globalThis.tdzRead = false;
        globalThis.tdzType = false;
        globalThis.tdzWrite = false;
        try { writeLate(); } catch (e) { tdzWrite = e instanceof ReferenceError; }
        try { readLate(); } catch (e) { tdzRead = e instanceof ReferenceError; }
        try { typeLate(); } catch (e) { tdzType = e instanceof ReferenceError; }
        let late = 42;
        const constant = 2, shadowed = 9;
    ",
        ))
        .unwrap();
    let value = context.eval(Source::from_bytes(r"
        writeLate();
        let immutable = false;
        try { writeConst(); } catch (e) { immutable = e instanceof TypeError; }
        tdzRead && tdzType && tdzWrite && evalLate() === 6 && readLate() === 43 && typeLate() === 'number'
            && readShadowed() === 9 && globalThis.shadowed === 1
            && constant === 2 && immutable && withLate() === 7
            && closed().join() === '8,43'
    ")).unwrap();
    assert_eq!(value.as_boolean(), Some(true));
}
