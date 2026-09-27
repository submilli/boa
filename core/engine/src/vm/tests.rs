use crate::error::RuntimeLimitError;
use crate::object::builtins::JsProxy;
use crate::vm::CallFrame;
use crate::vm::call_frame::CallFrameLocation;
use crate::vm::source_info::SourcePath;
use crate::{
    Context, JsNativeErrorKind, JsObject, JsValue, NativeFunction, TestAction, js_string,
    property::Attribute, run_test_actions, run_test_actions_with,
};
use boa_ast::Position;
use boa_macros::js_str;
use boa_parser::Source;
use indoc::indoc;
use std::fmt::Write;

#[test]
fn typeof_string() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            const a = "hello";
            typeof a;
        "#},
        js_str!("string"),
    )]);
}

#[test]
fn typeof_number() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            let a = 1234;
            typeof a;
        "#},
        js_str!("number"),
    )]);
}

#[test]
fn basic_op() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            const a = 1;
            const b = 2;
            a + b
        "#},
        3,
    )]);
}

#[test]
fn position() {
    let context = &mut Context::default();
    context
        .register_global_callable(
            js_string!("check_stack"),
            2,
            NativeFunction::from_copy_closure(|_, _, context| {
                let frame = context.stack_trace().collect::<Vec<&CallFrame>>();

                assert_eq!(frame.len(), 4);
                assert_eq!(
                    frame[0].position(),
                    CallFrameLocation {
                        function_name: js_string!("myOtherFunction"),
                        path: SourcePath::None,
                        position: Some(Position::new(2, 16))
                    }
                );
                assert_eq!(
                    frame[1].position(),
                    CallFrameLocation {
                        function_name: js_string!("<eval>"),
                        path: SourcePath::Eval,
                        position: Some(Position::new(1, 16))
                    }
                );
                assert_eq!(
                    frame[2].position(),
                    CallFrameLocation {
                        function_name: js_string!("myFunction"),
                        path: SourcePath::None,
                        position: Some(Position::new(5, 9))
                    }
                );
                assert_eq!(
                    frame[3].position(),
                    CallFrameLocation {
                        function_name: js_string!("<main>"),
                        path: SourcePath::None,
                        position: Some(Position::new(8, 11))
                    }
                );
                Ok(JsValue::undefined())
            }),
        )
        .expect("Could not register function");
    run_test_actions_with(
        [TestAction::run(indoc! {r#"
            const myOtherFunction = () => {
                check_stack();
            };
            function myFunction() {
                eval("myOtherFunction()");
            }

            myFunction();
        "#})],
        context,
    );
}

#[test]
fn try_catch_finally_from_init() {
    // the initialisation of the array here emits a PopOnReturnAdd op
    //
    // here we test that the stack is not popped more than intended due to multiple catches in the
    // same function, which could lead to VM stack corruption
    run_test_actions([TestAction::assert_opaque_error(
        indoc! {r#"
            try {
                [(() => {throw "h";})()];
            } catch (x) {
                throw "h";
            } finally {
            }
        "#},
        js_str!("h"),
    )]);
}

#[test]
fn multiple_catches() {
    // see explanation on `try_catch_finally_from_init`
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            try {
                try {
                    [(() => {throw "h";})()];
                } catch (x) {
                    throw "h";
                }
            } catch (y) {
            }
        "#},
        JsValue::undefined(),
    )]);
}

#[test]
fn use_last_expr_try_block() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            try {
                19;
                7.5;
                "Hello!";
            } catch (y) {
                14;
                "Bye!"
            }
        "#},
        js_str!("Hello!"),
    )]);
}

#[test]
fn use_last_expr_catch_block() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            try {
                throw Error("generic error");
                19;
                7.5;
            } catch (y) {
                14;
                "Hello!";
            }
        "#},
        js_str!("Hello!"),
    )]);
}

#[test]
fn no_use_last_expr_finally_block() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            try {
            } catch (y) {
            } finally {
                "Unused";
            }
        "#},
        JsValue::undefined(),
    )]);
}

#[test]
fn finally_block_binding_env() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            let buf = "Hey hey";
            try {
            } catch (y) {
            } finally {
                let x = " people";
                buf += x;
            }
            buf
        "#},
        js_str!("Hey hey people"),
    )]);
}

#[test]
fn run_super_method_in_object() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            let proto = {
                m() { return "super"; }
            };
            let obj = {
                v() { return super.m(); }
            };
            Object.setPrototypeOf(obj, proto);
            obj.v();
        "#},
        js_str!("super"),
    )]);
}

#[test]
fn get_reference_by_super() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            var fromA, fromB;
            var A = { fromA: 'a', fromB: 'a' };
            var B = { fromB: 'b' };
            Object.setPrototypeOf(B, A);
            var obj = {
                fromA: 'c',
                fromB: 'c',
                method() {
                    fromA = (() => { return super.fromA; })();
                    fromB = (() => { return super.fromB; })();
                }
            };
            Object.setPrototypeOf(obj, B);
            obj.method();
            fromA + fromB
        "#},
        js_str!("ab"),
    )]);
}

#[test]
fn super_call_constructor_null() {
    run_test_actions([TestAction::assert_native_error(
        indoc! {r#"
            class A extends Object {
                constructor() {
                    Object.setPrototypeOf(A, null);
                    super(A);
                }
            }
            new A();
        "#},
        JsNativeErrorKind::Type,
        "super constructor object must be constructor",
    )]);
}

#[test]
fn super_call_get_constructor_before_arguments_execution() {
    run_test_actions([TestAction::assert(indoc! {r#"
        class A extends Object {
            constructor() {
                super(Object.setPrototypeOf(A, null));
            }
        }
        new A() instanceof A;
    "#})]);
}

#[test]
fn order_of_execution_in_assignment() {
    run_test_actions([
        TestAction::run(indoc! {r#"
                let i = 0;
                let array = [[]];

                array[i++][i++] = i++;
            "#}),
        TestAction::assert_eq("i", 3),
        TestAction::assert_eq("array.length", 1),
        TestAction::assert_eq("array[0].length", 2),
    ]);
}

#[test]
fn order_of_execution_in_assignment_with_comma_expressions() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            let result = "";
            function f(i) {
                result += i;
            }
            let a = [[]];
            (f(1), a)[(f(2), 0)][(f(3), 0)] = (f(4), 123);
            result
        "#},
        js_str!("1234"),
    )]);
}

#[test]
fn loop_runtime_limit() {
    run_test_actions([
        TestAction::assert_eq(
            indoc! {r#"
                for (let i = 0; i < 20; ++i) { }
            "#},
            JsValue::undefined(),
        ),
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_loop_iteration_limit(10);
        }),
        TestAction::assert_runtime_limit_error(
            indoc! {r#"
                for (let i = 0; i < 20; ++i) { }
            "#},
            RuntimeLimitError::LoopIteration,
        ),
        TestAction::assert_eq(
            indoc! {r#"
                for (let i = 0; i < 10; ++i) { }
            "#},
            JsValue::undefined(),
        ),
        TestAction::assert_runtime_limit_error(
            indoc! {r#"
                while (1) { }
            "#},
            RuntimeLimitError::LoopIteration,
        ),
    ]);
}

#[test]
fn recursion_runtime_limit() {
    run_test_actions([
        TestAction::run(indoc! {r#"
            function factorial(n) {
                if (n == 0) {
                    return 1;
                }

                return n * factorial(n - 1);
            }
        "#}),
        TestAction::assert_eq("factorial(8)", JsValue::new(40_320)),
        TestAction::assert_eq("factorial(11)", JsValue::new(39_916_800)),
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_recursion_limit(10);
        }),
        TestAction::assert_native_error(
            "factorial(11)",
            JsNativeErrorKind::Range,
            "Maximum call stack size exceeded",
        ),
        TestAction::assert_eq("factorial(8)", JsValue::new(40_320)),
        TestAction::assert_native_error(
            indoc! {r#"
                function x() {
                    x()
                }

                x()
            "#},
            JsNativeErrorKind::Range,
            "Maximum call stack size exceeded",
        ),
    ]);
}

#[test]
fn arguments_object_constructor_valid_index() {
    run_test_actions([TestAction::assert_eq(
        indoc! {r#"
            let args;
            function F(a = 1) {
                args = arguments;
            }
            new F();
            typeof args
        "#},
        js_str!("object"),
    )]);
}

#[test]
fn empty_return_values() {
    run_test_actions([
        TestAction::run(indoc! {r#"do {{}} while (false);"#}),
        TestAction::run(indoc! {r#"do try {{}} catch {} while (false);"#}),
        TestAction::run(indoc! {r#"do {} while (false);"#}),
        TestAction::run(indoc! {r#"do try {{}{}} catch {} while (false);"#}),
        TestAction::run(indoc! {r#"do {{}{}} while (false);"#}),
        TestAction::run(indoc! {r#"do {;{}} while (false);"#}),
        TestAction::run(indoc! {r#"do {e: {}} while (false);"#}),
        TestAction::run(indoc! {r#"do {e: ;} while (false);"#}),
        TestAction::run(indoc! {r#"do { break } while (false);"#}),
        TestAction::run(indoc! {r#"while (true) a: break"#}),
        TestAction::run(indoc! {r#"while (true) a: {"a"; break};"#}),
        TestAction::run(indoc! {r#"do {"a";{}} while (false);"#}),
        TestAction::run(indoc! {r#"
            switch (false) {
                default: {}
            }
        "#}),
        TestAction::run(indoc! {r#"
            switch (false) {
                default: {}{}
            }
        "#}),
        TestAction::run(indoc! {r#"
            switch (false) {
                default: ;{}{}
            }
        "#}),
    ]);
}

#[test]
fn truncate_environments_on_non_caught_native_error() {
    let source = "with (new Proxy({}, {has: p => false})) {a}";
    run_test_actions([
        TestAction::assert_native_error(source, JsNativeErrorKind::Reference, "a is not defined"),
        TestAction::assert_native_error(source, JsNativeErrorKind::Reference, "a is not defined"),
    ]);
}

#[test]
fn super_construction_with_parameter_expression() {
    run_test_actions([
        TestAction::run(indoc! {r#"
            class Person {
                constructor(name) {
                    this.name = name;
                }
            }

            class Student extends Person {
                constructor(name = 'unknown') {
                    super(name);
                }
            }
        "#}),
        TestAction::assert_eq("new Student().name", js_str!("unknown")),
        TestAction::assert_eq("new Student('Jack').name", js_str!("Jack")),
    ]);
}

#[test]
fn cross_context_function_call() {
    let context1 = &mut Context::default();
    let result = context1.eval(Source::from_bytes(indoc! {r"
        var global = 100;

        (function x() {
            return global;
        })
    "}));

    assert!(result.is_ok());
    let result = result.unwrap();
    assert!(result.is_callable());

    let context2 = &mut Context::default();

    context2
        .register_global_property(js_string!("func"), result, Attribute::all())
        .unwrap();

    let result = context2.eval(Source::from_bytes("func()"));

    assert_eq!(result, Ok(JsValue::new(100)));
}

// See: https://github.com/boa-dev/boa/issues/1848
#[test]
fn long_object_chain_gc_trace_stack_overflow() {
    run_test_actions([
        TestAction::run(indoc! {r#"
            let old = {};
            for (let i = 0; i < 100000; i++) {
                old = { old };
            }
        "#}),
        TestAction::inspect_context(|_| boa_gc::force_collect()),
    ]);
}

// See: https://github.com/boa-dev/boa/issues/4515
#[test]
fn recursion_in_async_gen_rejects_with_range_error() {
    // The RangeError is thrown while resolving the return value, so (as in
    // browsers) it rejects the returned promise instead of escaping.
    run_test_actions([
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_recursion_limit(128);
        }),
        TestAction::assert(indoc! {r#"
            async function* f() {}
            f().return({
              get then() {
                this.then;
              },
            }) instanceof Promise
        "#}),
    ]);
}

#[test]
fn recursion_in_setter_throws_range_error() {
    run_test_actions([
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_recursion_limit(128);
        }),
        TestAction::assert_native_error(
            indoc! {r#"
                const obj = {
                  set x(value) {
                    this.x = value;
                  },
                };
                obj.x = 1;
            "#},
            JsNativeErrorKind::Range,
            "Maximum call stack size exceeded",
        ),
    ]);
}

#[test]
fn stack_overflow_is_a_catchable_range_error() {
    run_test_actions([
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_recursion_limit(128);
        }),
        TestAction::assert_eq(
            indoc! {r#"
                function f() { f() }
                let caught;
                try { f() } catch (e) { caught = e instanceof RangeError && e.message }
                caught
            "#},
            js_string!("Maximum call stack size exceeded"),
        ),
    ]);
}

#[test]
fn proxy_chains_count_toward_the_recursion_limit() {
    // A proxy whose target's prototype is the proxy itself forwards [[Get]]
    // forever without calling JS; it must fail with a RangeError rather than
    // overflow the native stack.
    run_test_actions([
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_recursion_limit(128);
        }),
        TestAction::assert_eq(
            indoc! {r#"
                const target = {};
                const proxy = new Proxy(target, {});
                Object.setPrototypeOf(target, proxy);
                let caught;
                try { proxy.missing } catch (e) { caught = e instanceof RangeError }
                caught
            "#},
            true,
        ),
    ]);
}

/// Run `test` on a thread with 8 mebibytes of native stack, the size embedders
/// commonly give script; without the limit, the recursion below overflows
/// it and aborts the process.
fn on_script_stack(test: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(8 << 20)
        .spawn(test)
        .expect("the test thread starts")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

#[test]
fn native_functions_calling_back_count_toward_the_recursion_limit() {
    // A native function that calls itself through `JsObject::call` never
    // runs bytecode, so only the host call depth can stop it.
    on_script_stack(|| {
        let context = &mut Context::default();
        context
            .register_global_callable(
                js_string!("reenter"),
                0,
                NativeFunction::from_copy_closure(|_, _, context| {
                    let this = context
                        .global_object()
                        .get(js_string!("reenter"), context)?;
                    let this = this.as_callable().expect("reenter is callable");
                    this.call(&JsValue::undefined(), &[], context)
                }),
            )
            .expect("the global is new");
        let result = context
            .eval(Source::from_bytes(
                "let caught; try { reenter() } catch (e) { caught = e instanceof RangeError } caught",
            ))
            .expect("the RangeError is caught");
        assert_eq!(result, JsValue::from(true));
    });
}

#[test]
fn a_native_proxy_trap_forwarding_through_reflect_in_a_cycle_is_a_range_error() {
    // An embedder's proxy trap forwarding to `Reflect.get`, with the proxy
    // in its own target's prototype chain: each level is a proxy method and
    // two native calls. The limit is an embedder's (the default stops
    // this recursion before it reaches the native stack's end anyway).
    on_script_stack(|| {
        let context = &mut Context::default();
        context.runtime_limits_mut().set_recursion_limit(5_000);
        let reflect_get = context
            .intrinsics()
            .objects()
            .reflect()
            .get(js_string!("get"), context)
            .expect("Reflect.get exists");
        context
            .register_global_property(js_string!("reflectGet"), reflect_get, Attribute::all())
            .expect("the global is new");
        let target = JsObject::with_object_proto(context.intrinsics());
        let proxy = JsProxy::builder(target.clone())
            .get(|_, args, context| {
                let forward = context
                    .global_object()
                    .get(js_string!("reflectGet"), context)?;
                let forward = forward.as_callable().expect("Reflect.get is callable");
                forward.call(&JsValue::undefined(), args, context)
            })
            .build(context)
            .expect("the proxy builds");
        let proxy = JsObject::from(proxy);
        target.set_prototype(Some(proxy.clone()));
        context
            .register_global_property(js_string!("cycle"), proxy, Attribute::all())
            .expect("the global is new");
        let result = context
            .eval(Source::from_bytes(
                "let caught; try { cycle.missing } catch (e) { caught = e instanceof RangeError } caught",
            ))
            .expect("the RangeError is caught");
        assert_eq!(result, JsValue::from(true));
    });
}

#[test]
fn value_stack_overflow_is_catchable_and_recovers() {
    // Frames with many locals fill the value stack before the call depth
    // limit. The RangeError must be catchable, and once it is caught the
    // stack must be back to the catching frame's size, so later calls in the
    // same script and in later scripts still work.
    let locals = (0..50).fold(String::new(), |mut locals, i| {
        write!(locals, "let v{i} = {i};").expect("writing to a String cannot fail");
        locals
    });
    run_test_actions([
        TestAction::inspect_context(|context| {
            context.runtime_limits_mut().set_stack_size_limit(10_000);
        }),
        TestAction::run(format!(
            "function deep() {{ {locals} return deep() + v0 }}\nfunction one() {{ return 1 }}"
        )),
        TestAction::assert_eq(
            indoc! {r#"
                let caught;
                try { deep() } catch (e) { caught = e instanceof RangeError && one() }
                caught
            "#},
            1,
        ),
        TestAction::assert_eq("one()", 1),
        TestAction::assert_native_error(
            "deep()",
            JsNativeErrorKind::Range,
            "Maximum call stack size exceeded",
        ),
        TestAction::assert_eq("one()", 1),
        TestAction::assert_eq(
            indoc! {r#"
                let calls = 0;
                for (let i = 0; i < 100; i++) {
                    try { deep() } catch { calls += one() }
                }
                calls
            "#},
            100,
        ),
    ]);
}

#[test]
fn return_value_survives_an_exception_caught_in_finally() {
    // A `return` inside `try` keeps its value on the stack while `finally`
    // runs; catching an exception inside that `finally` must not drop it.
    run_test_actions([
        TestAction::assert_eq(
            "(function () { try { return 42 } finally { try { throw 1 } catch {} } })()",
            42,
        ),
        TestAction::assert_eq(
            indoc! {r#"
                (function () {
                    let a = 7;
                    try { return a + 1 } finally { try { null.x } catch (e) {} }
                })()
            "#},
            8,
        ),
        TestAction::assert_eq(
            indoc! {r#"
                (function () {
                    for (const x of [1, 2]) {
                        try { return x * 10 } finally { try { throw 0 } catch {} }
                    }
                })()
            "#},
            10,
        ),
        TestAction::assert_eq(
            indoc! {r#"
                function* gen() { try { return 5 } finally { try { yield 1 } catch (e) {} } }
                const it = gen();
                it.next();
                it.throw(new Error('x')).value
            "#},
            5,
        ),
        TestAction::assert_eq(
            indoc! {r#"
                class Base {}
                class Derived extends Base {
                    constructor() {
                        super();
                        try { return { v: 42 } } finally { try { throw 1 } catch {} }
                    }
                }
                new Derived().v
            "#},
            42,
        ),
        TestAction::run(indoc! {r#"
            let resolved;
            (async function () { try { return 42 } finally { try { throw 1 } catch {} } })()
                .then((value) => { resolved = value });
        "#}),
        TestAction::inspect_context(|context| {
            context.run_jobs().expect("jobs run");
        }),
        TestAction::assert_eq("resolved", 42),
    ]);
}
