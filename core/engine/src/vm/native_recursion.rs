//! Native data walks have larger frames than bytecode calls and need a separate
//! ceiling. The context owns the counter so callbacks share the same budget.

use std::{cell::Cell, rc::Rc};

use crate::{Context, JsResult, error::RuntimeLimitError};

/// Kept independent of an embedder's potentially much larger bytecode limit.
const MAX_NATIVE_RECURSION: usize = 128;

/// Owns one active native level without borrowing the context across callbacks.
pub(crate) struct NativeRecursion(Rc<Cell<usize>>);

impl Context {
    /// Enter before reading page properties or calling script. The guard releases
    /// the level on both JavaScript errors and Rust unwinding.
    pub(crate) fn enter_native_recursion(&self) -> JsResult<NativeRecursion> {
        self.check_runtime_limits()?;
        let depth = &self.vm.native_recursion_depth;
        if depth.get() >= MAX_NATIVE_RECURSION {
            return Err(RuntimeLimitError::Recursion.into());
        }
        depth.set(depth.get() + 1);
        Ok(NativeRecursion(Rc::clone(depth)))
    }
}

impl Drop for NativeRecursion {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Context, TestAction, js_string, run_test_actions_with};

    fn on_script_stack(test: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 << 20)
            .spawn(test)
            .expect("the test thread starts")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
    }

    #[test]
    fn recursive_builtins_throw_range_error_and_recover() {
        on_script_stack(|| {
            let context = &mut Context::default();
            context.runtime_limits_mut().set_recursion_limit(5_000);
            run_test_actions_with(
                [
                    TestAction::run(
                        "var deep = []; for (let i = 0; i < 200000; i++) deep = [deep];",
                    ),
                    TestAction::assert(
                        "(() => { try { JSON.stringify(deep); return false } catch (e) { return e instanceof RangeError } })()",
                    ),
                    TestAction::assert(
                        "(() => { try { deep.flat(Infinity); return false } catch (e) { return e instanceof RangeError } })()",
                    ),
                    TestAction::assert(
                        "(() => { function g() { JSON.parse('[[[[1]]]]', g) } try { g(); return false } catch (e) { return e instanceof RangeError } })()",
                    ),
                    TestAction::assert_eq(
                        "JSON.stringify([1, {x: 2}])",
                        js_string!(r#"[1,{"x":2}]"#),
                    ),
                    TestAction::assert_eq("[1, [2]].flat(Infinity).join()", js_string!("1,2")),
                    TestAction::assert_eq("JSON.parse('[1]', (k, v) => v)[0]", 1),
                ],
                context,
            );
        });
    }

    #[test]
    fn data_walks_share_the_budget_through_callbacks_and_cycles() {
        on_script_stack(|| {
            let context = &mut Context::default();
            context.runtime_limits_mut().set_recursion_limit(5_000);
            run_test_actions_with(
                [
                    TestAction::run(
                        r#"
                    function rangeError(f) {
                        try { f(); return false } catch(e) { return e instanceof RangeError }
                    }
                "#,
                    ),
                    TestAction::assert(
                        "rangeError(() => { let o={}; for(let i=0;i<200000;i++) o={x:o}; JSON.stringify(o) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const a=[]; a[0]=a; a.flat(Infinity) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const o={toJSON(){return JSON.stringify(o)}}; JSON.stringify(o) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { function replace(k,v){return JSON.stringify(v, replace)}; JSON.stringify(1, replace) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const a=[]; Object.defineProperty(a, 0, {get(){return a.flat()}}); a.flat() })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { function map(){return [1].flatMap(map)}; [1].flatMap(map) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { JSON.parse('[0,1]', function(k,v){if(k==='0') this[1]=this; return v}) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const a=[{toJSON(){return a.flatMap(() => JSON.stringify(a))}}]; JSON.stringify(a) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const r=[];Object.defineProperty(r,0,{get(){return JSON.stringify(0,r)}});JSON.stringify(0,r) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { const a=[];Object.defineProperty(a,'constructor',{get(){return a.flat()}});a.flat() })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { let a=[];for(let i=0;i<200000;i++)a=[a];String(a) })",
                    ),
                    TestAction::assert(
                        "rangeError(() => { let a=[];for(let i=0;i<200000;i++)a=[a];a.join() })",
                    ),
                    TestAction::assert_eq("JSON.stringify({x:1})", js_string!(r#"{"x":1}"#)),
                    TestAction::assert_eq("[1,[2]].flat().join()", js_string!("1,2")),
                ],
                context,
            );
        });
    }

    #[test]
    fn ordinary_behavior_and_exception_cleanup_are_preserved() {
        run_test_actions_with(
            [
                TestAction::assert_eq(
                    "JSON.stringify({a:1,b:2}, (k,v) => k==='a' ? undefined : v)",
                    js_string!(r#"{"b":2}"#),
                ),
                TestAction::assert_eq(
                    "JSON.parse('[1,2]', (k,v) => k==='0' ? 3 : v).join()",
                    js_string!("3,2"),
                ),
                TestAction::assert_eq("[1,,[2,,[3]]].flat(2).join()", js_string!("1,2,3")),
                TestAction::assert(
                    "(() => {let a=[]; a.push(a); try { JSON.stringify(a); return false } catch(e) {return e instanceof TypeError} })()",
                ),
                TestAction::assert(
                    "(() => {const marker={}; for(let i=0;i<300;i++){try{JSON.stringify({get x(){throw marker}})}catch(e){if(e!==marker)return false}}return true})()",
                ),
                TestAction::assert_eq("JSON.stringify([1,2,3])", js_string!("[1,2,3]")),
            ],
            &mut Context::default(),
        );
    }

    #[test]
    fn guard_restores_depth_on_unwind_and_respects_embedder_limit() {
        let context = Context::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = context
                .enter_native_recursion()
                .expect("initial depth is zero");
            panic!("test unwind");
        }));
        assert!(result.is_err());
        assert_eq!(context.vm.native_recursion_depth.get(), 0);
        let mut context = context;
        context.runtime_limits_mut().set_recursion_limit(1);
        let guard = context
            .enter_native_recursion()
            .expect("one level is allowed");
        assert!(context.enter_native_recursion().is_err());
        drop(guard);
        assert!(context.enter_native_recursion().is_ok());
    }
}
