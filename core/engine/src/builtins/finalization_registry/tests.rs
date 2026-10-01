mod miri {

    use indoc::indoc;

    use crate::{TestAction, run_test_actions};

    #[test]
    fn finalization_registry_simple() {
        run_test_actions([
            TestAction::run(indoc! {r#"
            let counter = 0;
            const registry = new FinalizationRegistry(() => {
                counter++;
            });

            registry.register(["foo"]);
        "#}),
            TestAction::assert_eq("counter", 0),
            TestAction::inspect_context(|_| boa_gc::force_collect()),
            TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
            // Callback should run at least once
            TestAction::assert_eq("counter", 1),
        ]);
    }

    #[test]
    fn finalization_registry_unregister() {
        run_test_actions([
            TestAction::run(indoc! {r#"
            let counter = 0;
            const registry = new FinalizationRegistry(() => {
                counter++;
            });

            {
                let array = ["foo"];
                registry.register(array, undefined, array);
                registry.unregister(array);
            }

        "#}),
            TestAction::assert_eq("counter", 0),
            TestAction::inspect_context(|_| boa_gc::force_collect()),
            TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
            // Callback shouldn't run
            TestAction::assert_eq("counter", 0),
        ]);
    }

    #[test]
    fn finalization_registry_held_value_handover() {
        run_test_actions([
            TestAction::run(indoc! {r#"
            let counter = 0;
            const registry = new FinalizationRegistry((value) => {
                counter += value.increment;
            });

            registry.register(["foo"], { increment: 5 });
        "#}),
            TestAction::assert_eq("counter", 0),
            TestAction::inspect_context(|_| boa_gc::force_collect()),
            TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
            // Registry should handover the held value as argument
            TestAction::assert_eq("counter", 5),
        ]);
    }

    #[test]
    fn finalization_registry_unrelated_unregister_token() {
        run_test_actions([
            TestAction::run(indoc! {r#"
            let counter = 0;

            const registry = new FinalizationRegistry((value) => {
                counter += 1;
            });

            registry.register(["foo"], undefined, {});
            registry.unregister({});
        "#}),
            TestAction::assert_eq("counter", 0),
            TestAction::inspect_context(|_| boa_gc::force_collect()),
            TestAction::inspect_context(|ctx| ctx.run_jobs().unwrap()),
            // Object should not have been unregistered if the token is not the correct one
            TestAction::assert_eq("counter", 1),
        ]);
    }
}

mod persistent {
    use crate::{Context, Source};

    fn eval(context: &mut Context, source: &str) {
        context.eval(Source::from_bytes(source)).unwrap();
    }

    fn assert_true(context: &mut Context, source: &str) {
        assert_eq!(
            context
                .eval(Source::from_bytes(source))
                .unwrap()
                .as_boolean(),
            Some(true)
        );
    }

    #[test]
    fn idle_checkpoints_retain_notifications_and_rearm_after_cleanup() {
        let mut context = Context::default();
        eval(
            &mut context,
            "var values=[]; var registry=new FinalizationRegistry(v=>values.push(v)); var target={}; registry.register(target, 1);",
        );
        for _ in 0..3 {
            context.run_jobs().unwrap();
        }
        eval(
            &mut context,
            "target=null; Promise.resolve().then(()=>values.push('promise'));",
        );
        boa_gc::force_collect();
        context.run_jobs().unwrap();
        assert_true(&mut context, "values.join() === 'promise,1'");
        eval(&mut context, "target={}; registry.register(target, 2);");
        context.run_jobs().unwrap();
        eval(&mut context, "target=null");
        boa_gc::force_collect();
        context.run_jobs().unwrap();
        assert_true(&mut context, "values.join() === 'promise,1,2'");
    }

    #[test]
    fn throwing_cleanup_does_not_strand_coalesced_notifications() {
        let mut context = Context::default();
        eval(
            &mut context,
            "var calls=0; var promises=0; var registry=new FinalizationRegistry(()=>{ Promise.resolve().then(()=>promises++); if (++calls===1) throw 1; }); registry.register({},1); registry.register({},2);",
        );
        context.run_jobs().unwrap();
        boa_gc::force_collect();
        assert!(context.run_jobs().is_err());
        context.run_jobs().unwrap();
        assert_true(&mut context, "calls === 2 && promises === 2");
    }

    #[test]
    fn idle_handles_do_not_root_registry_or_held_values() {
        let mut context = Context::default();
        eval(
            &mut context,
            "var target={}; var held={}; var registry=new FinalizationRegistry(()=>{}); registry.register(target,held); var weakRegistry=new WeakRef(registry); var weakHeld=new WeakRef(held); registry=null; held=null;",
        );
        context.run_jobs().unwrap();
        boa_gc::force_collect();
        context.run_jobs().unwrap();
        boa_gc::force_collect();
        assert_true(
            &mut context,
            "weakRegistry.deref()===undefined && weakHeld.deref()===undefined",
        );
    }

    #[test]
    fn cleanup_handle_admission_is_bounded_and_gc_reclaims_capacity() {
        let mut context = Context::default();
        eval(
            &mut context,
            "var registries=[]; for(let i=0;i<4096;i++) registries.push(new FinalizationRegistry(()=>{})); var limited=false; try { new FinalizationRegistry(()=>{}); } catch(e) { limited=e instanceof RangeError; }",
        );
        assert_true(&mut context, "limited");
        context.run_jobs().unwrap();
        eval(&mut context, "registries=null");
        boa_gc::force_collect();
        eval(
            &mut context,
            "var replacement=new FinalizationRegistry(()=>{});",
        );
        context.run_jobs().unwrap();
    }

    #[test]
    fn registry_cell_admission_is_bounded_and_unregister_reclaims_capacity() {
        let mut context = Context::default();
        eval(
            &mut context,
            "var registry=new FinalizationRegistry(()=>{}); var target={}; for(let i=0;i<65536;i++) registry.register(target,i,target); var limited=false; try { registry.register(target,65536,target); } catch(e) { limited=e instanceof RangeError; }",
        );
        assert_true(&mut context, "limited && registry.unregister(target)");
        eval(&mut context, "registry.register(target,1,target)");
    }
}
