use boa_macros::js_str;
use std::fmt::Write;

use crate::{TestAction, run_test_actions};

fn declarations() -> String {
    let mut source = String::new();
    for i in 0..128 {
        writeln!(source, "let binding_{i} = {i};").unwrap();
    }
    source
}

#[test]
fn large_scope_closures_and_direct_eval_keep_binding_slots() {
    let source = format!(
        r#"(function(argument) {{
            {}
            var duplicate = 5; var duplicate;
            const immutable = 9;
            let constantError = false;
            try {{ immutable = 10; }} catch (e) {{ constantError = e instanceof TypeError; }}
            const read = () => binding_0 + binding_63 + binding_127;
            eval('binding_63 = 100; var introduced = 7;');
            let shadow;
            {{ let binding_127 = 3; shadow = () => binding_127; }}
            return [read(), shadow(), binding_127, duplicate, immutable,
                    constantError, introduced, arguments[0]].join(',');
        }})(11)"#,
        declarations(),
    );
    run_test_actions([TestAction::assert_eq(
        source,
        js_str!("227,3,127,5,9,true,7,11"),
    )]);
}

#[test]
fn large_scope_captures_skip_uncaptured_slots() {
    let source = format!(
        r#"(function() {{
            {}
            function first() {{ return binding_0; }}
            function last() {{ binding_127 += 1; return binding_127; }}
            {{ let binding_63 = 400; if (binding_63 !== 400) throw Error('shadow'); }}
            return [first(), last(), binding_63, last()].join(',');
        }})()"#,
        declarations(),
    );
    run_test_actions([TestAction::assert_eq(source, js_str!("0,128,63,129"))]);
}

#[test]
fn large_scope_tdz_and_dynamic_with_lookup_remain_observable() {
    let source = format!(
        r#"(function() {{
            {}
            let tdz = false;
            {{ try {{ binding_127; }} catch (e) {{ tdz = e instanceof ReferenceError; }}
               let binding_127 = 600; }}
            let dynamic;
            with ({{ binding_127: 900 }}) {{ dynamic = binding_127; }}
            return [tdz, dynamic, binding_127].join(',');
        }})()"#,
        declarations(),
    );
    run_test_actions([TestAction::assert_eq(source, js_str!("true,900,127"))]);
}
