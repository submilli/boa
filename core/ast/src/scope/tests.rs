use super::{BindingLocatorError, Scope};
use boa_string::JsString;

fn name(i: u32) -> JsString {
    JsString::from(format!("binding_{i}"))
}

fn populated(count: u32) -> Scope {
    let scope = Scope::new(Scope::new_global(), true);
    for i in 0..count {
        let locator = scope.create_mutable_binding(name(i), true);
        assert_eq!(locator.binding_index(), i);
    }
    scope
}

#[test]
fn indexed_lookup_preserves_declaration_order_and_duplicate_slots() {
    // Exercise promotion as well as later inserts, with deliberately nonlexical
    // name order (binding_10 sorts before binding_2).
    for count in [0, 1, 31, 32, 33, 128, 1024] {
        let scope = populated(count);
        let clone = scope.clone();
        for i in 0..count {
            let locator = scope.get_binding(&name(i)).unwrap();
            assert_eq!(locator.binding_index(), i);
            assert_eq!(scope.create_mutable_binding(name(i), false), locator);
            assert_eq!(clone.get_identifier_reference(name(i)).locator(), locator);
            assert!(!scope.has_lex_binding(&name(i)));
        }
        assert_eq!(scope.num_bindings(), count);
        assert!(!scope.has_binding(&name(count)));
        assert!(scope.get_binding(&name(count)).is_none());
        assert!(
            scope
                .get_identifier_reference(name(count))
                .is_global_object()
        );
    }
}

#[test]
fn indexed_names_survive_escape_slot_reordering_and_later_insertion() {
    let scope = populated(128);
    let child = Scope::new(scope.clone(), true);
    for i in (0..128).step_by(3) {
        child.access_binding(&name(i), false);
    }
    scope.reorder_binding_indices();
    for i in 0..128 {
        let reference = scope.get_binding_reference(&name(i)).unwrap();
        assert_eq!(reference.local(), i % 3 != 0);
        assert_eq!(
            reference.locator().binding_index(),
            if i % 3 == 0 { i / 3 } else { 0 }
        );
        assert_eq!(
            scope.create_mutable_binding(name(i), true),
            reference.locator()
        );
    }
    let extra = scope.create_mutable_binding(name(128), true);
    assert_eq!(extra.binding_index(), 128);
    scope.access_binding(&name(128), true);
    scope.reorder_binding_indices();
    assert_eq!(scope.get_binding(&name(128)).unwrap().binding_index(), 43);
    assert_eq!(scope.num_bindings_non_local(), 44);
}

#[test]
fn indexed_lookup_preserves_shadowing_mutability_and_escape_boundaries() {
    let outer = populated(128);
    let child = Scope::new(outer.clone(), false);
    for i in 64..192 {
        drop(child.create_mutable_binding(name(i), false));
    }
    child.access_binding(&name(64), false);
    assert!(outer.get_binding_reference(&name(64)).unwrap().local());
    assert!(child.get_binding_reference(&name(64)).unwrap().local());
    assert!(child.has_lex_binding(&name(64)));
    assert_eq!(
        child
            .get_identifier_reference(name(64))
            .locator()
            .unique_scope_id(),
        child.unique_id()
    );
    assert_eq!(
        child
            .get_identifier_reference(name(0))
            .locator()
            .unique_scope_id(),
        outer.unique_id()
    );
    child.access_binding(&name(0), true);
    assert!(!outer.get_binding_reference(&name(0)).unwrap().local());
    let function = Scope::new(child.clone(), true);
    function.access_binding(&name(64), false);
    assert!(!child.get_binding_reference(&name(64)).unwrap().local());
    assert!(outer.get_binding_reference(&name(64)).unwrap().local());
    assert_eq!(child.is_binding_mutable(&name(0)), Some(true));
    assert_eq!(child.is_binding_mutable(&name(500)), None);
    assert_eq!(
        child.set_mutable_binding(name(0)).unwrap().locator(),
        outer.get_binding(&name(0)).unwrap()
    );
}

#[test]
fn indexed_immutable_bindings_keep_strictness_and_do_not_duplicate() {
    let scope = populated(64);
    let strict = JsString::from("strict");
    let silent = JsString::from("silent");
    scope.create_immutable_binding(strict.clone(), true);
    scope.create_immutable_binding(silent.clone(), false);
    scope.create_immutable_binding(strict.clone(), false);
    drop(scope.create_mutable_binding(strict.clone(), true));
    assert_eq!(scope.num_bindings(), 66);
    assert_eq!(scope.is_binding_mutable(&strict), Some(false));
    assert!(scope.has_lex_binding(&strict));
    assert!(matches!(
        scope.set_mutable_binding(strict),
        Err(BindingLocatorError::MutateImmutable)
    ));
    assert!(matches!(
        scope.set_mutable_binding(silent),
        Err(BindingLocatorError::Silent)
    ));
    // Duplicate immutable creation must not change an existing mutable binding.
    scope.create_immutable_binding(name(10), true);
    assert_eq!(scope.is_binding_mutable(&name(10)), Some(true));
}

#[test]
fn indexed_names_compare_by_string_value_including_unicode() {
    let scope = populated(64);
    for text in ["arguments", "café", "αβ", "𐐀"] {
        let original = JsString::from(text);
        let units: Vec<_> = text.encode_utf16().collect();
        let equivalent = JsString::from(units.as_slice());
        let locator = scope.create_mutable_binding(original, true);
        assert_eq!(scope.get_binding(&equivalent), Some(locator.clone()));
        assert_eq!(scope.create_mutable_binding(equivalent, true), locator);
    }
    assert_eq!(scope.num_bindings(), 68);
}

#[cfg(feature = "annex-b")]
#[test]
fn indexed_var_assignment_skips_lexical_scopes() {
    let outer = populated(64);
    let block = Scope::new(outer.clone(), false);
    for i in 0..64 {
        drop(block.create_mutable_binding(name(i), false));
    }
    assert_eq!(
        block.set_mutable_binding_var(name(10)).unwrap().locator(),
        outer.get_binding(&name(10)).unwrap()
    );
    assert_eq!(
        block.set_mutable_binding(name(10)).unwrap().locator(),
        block.get_binding(&name(10)).unwrap()
    );
}
