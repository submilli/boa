use crate::{JsObject, JsSymbol, object::shape::slot::SlotAttributes, property::PropertyKey};

use super::{SharedShape, TransitionKey};

#[test]
fn dead_prototype_transitions_do_not_retain_the_prototype() {
    let root = SharedShape::root();
    let prototype = JsObject::with_null_proto();
    let weak = boa_gc::WeakGc::new(prototype.inner());
    let shape = root.change_prototype_transition(Some(prototype.clone()));
    drop(prototype);
    boa_gc::force_collect();
    assert!(weak.upgrade().is_some(), "a live shape owns its prototype");
    drop(shape);
    boa_gc::force_collect();
    assert!(weak.upgrade().is_none(), "the transition cache is weak");
    // Keep the transition source alive across collection.
    assert_eq!(
        root.forward_transitions().prototype_transitions_count().0,
        1
    );
}

#[test]
fn prototype_transitions_reuse_live_shapes_and_replace_expired_shapes() {
    let root = SharedShape::root();
    let prototype = JsObject::with_null_proto();
    let first = root.change_prototype_transition(Some(prototype.clone()));
    let second = root.change_prototype_transition(Some(prototype.clone()));
    assert!(boa_gc::Gc::ptr_eq(&first.inner, &second.inner));
    let weak = boa_gc::WeakGc::new(&first.inner);
    drop(first);
    drop(second);
    boa_gc::force_collect();
    assert!(weak.upgrade().is_none());
    let replacement = root.change_prototype_transition(Some(prototype.clone()));
    assert_eq!(replacement.prototype(), Some(prototype));
    assert_eq!(
        root.forward_transitions().prototype_transitions_count().0,
        1
    );
}

#[test]
fn shared_root_shapes_do_not_retain_discarded_realms() {
    use crate::{Context, Finalize, JsData, Trace};
    #[derive(Trace, Finalize, JsData)]
    struct Marker(boa_gc::Gc<u8>);
    let mut context = Context::default();
    let realm = context.create_realm().unwrap();
    let marker = boa_gc::Gc::new(0);
    let weak = boa_gc::WeakGc::new(&marker);
    realm.host_defined_mut().insert(Marker(marker));
    drop(realm);
    boa_gc::force_collect();
    assert!(weak.upgrade().is_none());
    // The original realm remains usable after collecting the child.
    assert_eq!(
        context.eval(crate::Source::from_bytes("1 + 1")).unwrap(),
        2.into()
    );
}

#[test]
fn test_prune_property_on_counter_limit() {
    let shape = SharedShape::root();

    for i in 0..255 {
        assert_eq!(
            shape.forward_transitions().property_transitions_count(),
            (i, i as u8)
        );

        shape.insert_property_transition(TransitionKey {
            property_key: PropertyKey::Symbol(JsSymbol::new(None).unwrap()),
            attributes: SlotAttributes::all(),
        });
    }

    assert_eq!(
        shape.forward_transitions().property_transitions_count(),
        (255, 255)
    );

    boa_gc::force_collect();

    {
        shape.insert_property_transition(TransitionKey {
            property_key: PropertyKey::Symbol(JsSymbol::new(None).unwrap()),
            attributes: SlotAttributes::all(),
        });
    }

    assert_eq!(
        shape.forward_transitions().property_transitions_count(),
        (1, 0)
    );

    {
        shape.insert_property_transition(TransitionKey {
            property_key: PropertyKey::Symbol(JsSymbol::new(None).unwrap()),
            attributes: SlotAttributes::all(),
        });
    }

    assert_eq!(
        shape.forward_transitions().property_transitions_count(),
        (2, 1)
    );

    boa_gc::force_collect();

    assert_eq!(
        shape.forward_transitions().property_transitions_count(),
        (2, 1)
    );
}

#[test]
fn test_prune_prototype_on_counter_limit() {
    let shape = SharedShape::root();

    assert_eq!(
        shape.forward_transitions().prototype_transitions_count(),
        (0, 0)
    );

    for i in 0..255 {
        assert_eq!(
            shape.forward_transitions().prototype_transitions_count(),
            (i, i as u8)
        );

        shape.change_prototype_transition(Some(JsObject::with_null_proto()));
    }

    // Allocate the next key while the old prototypes are alive: this test
    // exercises counter-triggered pruning, not an expired address cache hit.
    let next_prototype = JsObject::with_null_proto();
    boa_gc::force_collect();

    assert_eq!(
        shape.forward_transitions().prototype_transitions_count(),
        (255, 255)
    );

    {
        shape.change_prototype_transition(Some(next_prototype));
    }

    assert_eq!(
        shape.forward_transitions().prototype_transitions_count(),
        (1, 0)
    );

    {
        shape.change_prototype_transition(Some(JsObject::with_null_proto()));
    }

    assert_eq!(
        shape.forward_transitions().prototype_transitions_count(),
        (2, 1)
    );

    boa_gc::force_collect();

    assert_eq!(
        shape.forward_transitions().prototype_transitions_count(),
        (2, 1)
    );
}
