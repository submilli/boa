use crate::{JsNativeErrorKind, TestAction, run_test_actions};
use indoc::indoc;

#[test]
fn ordinary_has_instance_nonobject_prototype() {
    run_test_actions([TestAction::assert_native_error(
        indoc! {r#"
            function C() {}
            C.prototype = 1
            String instanceof C
        "#},
        JsNativeErrorKind::Type,
        "function has non-object prototype in instanceof check",
    )]);
}

#[test]
fn object_properties_return_order() {
    run_test_actions([
        TestAction::run_harness(),
        TestAction::run(indoc! {r#"
                var o = {
                    p1: 'v1',
                    p2: 'v2',
                    p3: 'v3',
                };
                o.p4 = 'v4';
                o[2] = 'iv2';
                o[0] = 'iv0';
                o[1] = 'iv1';
                delete o.p1;
                delete o.p3;
                o.p1 = 'v1';
            "#}),
        TestAction::assert(r#"arrayEquals(Object.keys(o), [ "0", "1", "2", "p2", "p4", "p1" ])"#),
        TestAction::assert(
            r#"arrayEquals(Object.values(o), [ "iv0", "iv1", "iv2", "v2", "v4", "v1" ])"#,
        ),
    ]);
}

#[test]
fn callable_realm_resolves_targets_without_running_script() {
    use crate::{Context, Source};
    let context = &mut Context::default();
    let caller = context.realm().clone();
    let owner = context.create_realm().unwrap();
    context.enter_realm(owner.clone());
    let ordinary = context.eval(Source::from_bytes("(() => 1)")).unwrap();
    let native = context.eval(Source::from_bytes("Object")).unwrap();
    let bound = context
        .eval(Source::from_bytes("(() => 1).bind(null).bind(null)"))
        .unwrap();
    let proxy = context
        .eval(Source::from_bytes(
            "new Proxy((() => 1).bind(null), { get() { throw 'trap'; } })",
        ))
        .unwrap();
    let revoked = context
        .eval(Source::from_bytes(
            "(() => { const p = Proxy.revocable(() => 1, {}); p.revoke(); return p.proxy; })()",
        ))
        .unwrap();
    context.enter_realm(caller.clone());
    for value in [ordinary, native, bound, proxy] {
        assert_eq!(value.as_function().unwrap().realm(context).unwrap(), owner);
        assert_eq!(*context.realm(), caller);
    }
    assert!(
        revoked
            .as_function()
            .unwrap()
            .realm(context)
            .unwrap_err()
            .as_native()
            .unwrap()
            .is_type()
    );
    assert_eq!(*context.realm(), caller);
}
