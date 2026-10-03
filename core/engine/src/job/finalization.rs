//! Owned GC notification handles; no suspended Context borrow.

use crate::{
    Context, JsObject, JsResult, JsValue, builtins::finalization_registry::FinalizationRegistry,
    object::VTableObject,
};
use boa_gc::WeakGc;
use std::{cell::Cell, rc::Rc};

/// A persistent, nonblocking `FinalizationRegistry` notification handle.
///
/// Retain this handle while `is_finished` is false. An idle handle is not
/// runnable work and must not keep an event loop alive. It roots neither the
/// registry nor its targets. One call delivers at most one cleanup callback.
#[derive(Clone)]
pub struct FinalizationRegistryCleanupJob {
    registry: WeakGc<VTableObject<FinalizationRegistry>>,
    receiver: async_channel::Receiver<()>,
    notified: Rc<Cell<bool>>,
    running: Rc<Cell<bool>>,
}

impl FinalizationRegistryCleanupJob {
    pub(crate) fn new(
        registry: WeakGc<VTableObject<FinalizationRegistry>>,
        receiver: async_channel::Receiver<()>,
    ) -> Self {
        Self {
            registry,
            receiver,
            notified: Rc::new(Cell::new(false)),
            running: Rc::new(Cell::new(false)),
        }
    }

    /// Resolve the cleanup callback's realm without retaining it in this handle.
    /// Returns `None` after collection. A revoked callback uses the registry's
    /// creation realm for reporting the ensuing invocation error.
    pub fn realm(&self, context: &Context) -> Option<crate::realm::Realm> {
        let registry = self.registry.upgrade().map(JsObject::from_inner)?;
        Some(registry.borrow().data().callback_realm(context))
    }

    /// Whether the registry has been collected and the handle can be discarded.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.registry.upgrade().is_none()
    }

    /// Whether GC has signaled cleanup. This never runs script or waits.
    #[must_use]
    pub fn is_ready(&self) -> bool {
        !self.running.get()
            && (self.notified.get() || !self.receiver.is_empty())
            && !self.is_finished()
    }

    /// Wait asynchronously for a GC notification or registry collection.
    ///
    /// This borrows no Context. A consumed notification is retained in the
    /// handle, so dropping a ready waiter never loses pending cleanup.
    pub async fn wait_until_ready(&self) {
        if self.is_ready() || self.is_finished() {
            return;
        }
        if self.receiver.recv().await.is_ok() {
            self.notified.set(true);
        }
    }

    /// Deliver one ready cleanup callback, if any, without waiting.
    ///
    /// The handle remains valid after success or failure. Hosts should resume
    /// ordinary microtasks before checking for another cleanup callback.
    pub fn call(&self, context: &mut Context) -> JsResult<JsValue> {
        if self.running.get()
            || (!self.notified.replace(false) && self.receiver.try_recv().is_err())
        {
            return Ok(JsValue::undefined());
        }
        let Some(registry) = self.registry.upgrade().map(JsObject::from_inner) else {
            return Ok(JsValue::undefined());
        };
        self.running.set(true);
        let result = FinalizationRegistry::cleanup(&registry, context);
        self.running.set(false);
        result.map(|()| JsValue::undefined())
    }
}

impl std::fmt::Debug for FinalizationRegistryCleanupJob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FinalizationRegistryCleanupJob")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Source,
        job::{Job, JobExecutor},
    };
    use std::{
        cell::RefCell,
        future::Future,
        pin::pin,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        task::{Poll, Wake, Waker},
    };

    #[derive(Default)]
    struct Capture(RefCell<Vec<FinalizationRegistryCleanupJob>>);

    impl JobExecutor for Capture {
        fn enqueue_job(self: Rc<Self>, job: Job, _: &mut Context) {
            if let Job::FinalizationRegistryCleanupJob(job) = job {
                self.0.borrow_mut().push(job);
            }
        }
        fn run_jobs(self: Rc<Self>, _: &mut Context) -> JsResult<()> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct Notification(AtomicBool);
    impl Wake for Notification {
        fn wake(self: Arc<Self>) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn gc_wakes_owned_waiter_and_cancellation_preserves_notification() {
        let executor = Rc::new(Capture::default());
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .build()
            .unwrap();
        context.eval(Source::from_bytes("var cleaned=false; var registry=new FinalizationRegistry(()=>cleaned=true); var target={}; registry.register(target,1);")).unwrap();
        let job = executor.0.borrow()[0].clone();
        assert_eq!(job.realm(&context), Some(context.realm().clone()));
        let notification = Arc::new(Notification::default());
        let waker = Waker::from(notification.clone());
        let mut task = std::task::Context::from_waker(&waker);
        {
            let mut waiter = pin!(job.wait_until_ready());
            assert!(matches!(waiter.as_mut().poll(&mut task), Poll::Pending));
            context.eval(Source::from_bytes("target=null")).unwrap();
            boa_gc::force_collect();
            assert!(notification.0.load(Ordering::Relaxed));
            assert!(matches!(waiter.as_mut().poll(&mut task), Poll::Ready(())));
        }
        assert!(job.is_ready());
        job.call(&mut context).unwrap();
        assert_eq!(
            context
                .eval(Source::from_bytes("cleaned"))
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert!(!job.is_ready());
        drop(context);
        boa_gc::force_collect();
        boa_gc::force_collect();
        assert!(job.is_finished());
        assert!(job.realm(&Context::default()).is_none());
    }
}
