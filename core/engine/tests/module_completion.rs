//! Owned host module completions across independent engine turns.
// Integration tests use only a subset of the engine crate dependencies.
#![allow(unused_crate_dependencies)]

use std::{cell::RefCell, collections::VecDeque, rc::Rc};

use boa_engine::{
    Context, JsNativeError, JsResult, Module, Source,
    builtins::promise::PromiseState,
    module::{ModuleLoadCompletion, ModuleLoader, ModuleRequest, Referrer},
};

#[derive(Default)]
struct DeferredLoader {
    requests: RefCell<VecDeque<(String, ModuleLoadCompletion)>>,
}
impl ModuleLoader for DeferredLoader {
    fn load_imported_module(
        self: Rc<Self>,
        _: Referrer,
        _: ModuleRequest,
        _: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        std::future::ready(Err(JsNativeError::typ()
            .with_message("Unexpected async loader call")
            .into()))
    }

    fn load_imported_module_with_completion(
        self: Rc<Self>,
        _: Referrer,
        request: ModuleRequest,
        completion: ModuleLoadCompletion,
        _: &mut Context,
    ) {
        self.requests
            .borrow_mut()
            .push_back((request.specifier().to_std_string_escaped(), completion));
    }
}

#[test]
fn deferred_dynamic_load_survives_context_calls_and_restores_realm() {
    let loader = Rc::new(DeferredLoader::default());
    let context = &mut Context::builder()
        .module_loader(loader.clone())
        .build()
        .unwrap();
    context
        .eval(Source::from_bytes(
            "globalThis.answer = 0; import('dep').then(m => answer = m.value)",
        ))
        .unwrap();
    context.run_jobs().unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes("answer"))
            .unwrap()
            .as_number(),
        Some(0.0)
    );
    let (name, completion) = loader.requests.borrow_mut().pop_front().unwrap();
    assert_eq!(name, "dep");
    boa_gc::force_collect();
    let module =
        Module::parse(Source::from_bytes("export const value = 42"), None, context).unwrap();
    let initiating = context.realm().clone();
    let other = context.create_realm().unwrap();
    context.enter_realm(other.clone());
    completion.complete(Ok(module), context);
    assert_eq!(context.realm(), &other);
    context.run_jobs().unwrap();
    assert_eq!(context.realm(), &other);
    context.enter_realm(initiating);
    assert_eq!(
        context
            .eval(Source::from_bytes("answer"))
            .unwrap()
            .as_number(),
        Some(42.0)
    );
}

#[test]
fn deferred_static_graph_and_failed_dynamic_import_settle() {
    let loader = Rc::new(DeferredLoader::default());
    let context = &mut Context::builder()
        .module_loader(loader.clone())
        .build()
        .unwrap();
    let entry = Module::parse(
        Source::from_bytes("import {value} from 'dep'; globalThis.answer = value"),
        None,
        context,
    )
    .unwrap();
    let promise = entry.load_link_evaluate(context);
    context.run_jobs().unwrap();
    assert!(matches!(promise.state(), PromiseState::Pending));
    let (_, completion) = loader.requests.borrow_mut().pop_front().unwrap();
    let dependency = Module::parse(
        Source::from_bytes("export {value} from 'leaf'"),
        None,
        context,
    )
    .unwrap();
    completion.complete(Ok(dependency), context);
    context.run_jobs().unwrap();
    let (name, completion) = loader.requests.borrow_mut().pop_front().unwrap();
    assert_eq!(name, "leaf");
    let leaf = Module::parse(Source::from_bytes("export const value = 7"), None, context).unwrap();
    completion.complete(Ok(leaf), context);
    context.run_jobs().unwrap();
    assert!(matches!(promise.state(), PromiseState::Fulfilled(_)));
    assert_eq!(
        context
            .eval(Source::from_bytes("answer"))
            .unwrap()
            .as_number(),
        Some(7.0)
    );

    context
        .eval(Source::from_bytes(
            "globalThis.failed = false; import('missing').catch(() => failed = true)",
        ))
        .unwrap();
    let (_, completion) = loader.requests.borrow_mut().pop_front().unwrap();
    completion.complete(
        Err(JsNativeError::typ().with_message("missing").into()),
        context,
    );
    context.run_jobs().unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes("failed"))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}

struct ImmediateLoader;
impl ModuleLoader for ImmediateLoader {
    fn load_imported_module(
        self: Rc<Self>,
        _: Referrer,
        _: ModuleRequest,
        _: &RefCell<&mut Context>,
    ) -> impl Future<Output = JsResult<Module>> {
        std::future::ready(Err(JsNativeError::typ()
            .with_message("Unexpected async loader call")
            .into()))
    }
    fn load_imported_module_with_completion(
        self: Rc<Self>,
        _: Referrer,
        request: ModuleRequest,
        completion: ModuleLoadCompletion,
        context: &mut Context,
    ) {
        let depth: usize = request.specifier().to_std_string_escaped().parse().unwrap();
        let source = if depth < 4096 {
            format!("import '{}';", depth + 1)
        } else {
            String::new()
        };
        let module = Module::parse(Source::from_bytes(&source), None, context);
        completion.complete(module, context);
    }
}
#[test]
fn immediate_completion_does_not_recurse_through_loading() {
    let context = &mut Context::builder()
        .module_loader(Rc::new(ImmediateLoader))
        .build()
        .unwrap();
    let entry = Module::parse(Source::from_bytes("import '0'"), None, context).unwrap();
    // Only loading is under test: linking/evaluation have separate traversal rules.
    let loaded = entry.load(context);
    context.run_jobs().unwrap();
    assert!(matches!(loaded.state(), PromiseState::Fulfilled(_)));
}

#[test]
fn existing_async_loader_uses_the_default_adapter() {
    let context = &mut Context::builder()
        .module_loader(Rc::new(boa_engine::module::IdleModuleLoader))
        .build()
        .unwrap();
    context
        .eval(Source::from_bytes(
            "globalThis.failed = false; import('missing').catch(() => failed = true)",
        ))
        .unwrap();
    context.run_jobs().unwrap();
    assert_eq!(
        context
            .eval(Source::from_bytes("failed"))
            .unwrap()
            .as_boolean(),
        Some(true)
    );
}
