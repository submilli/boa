use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    mem,
    pin::{Pin, pin},
    rc::Rc,
};

use boa_engine::{
    Context, JsResult, JsValue,
    job::{
        FinalizationRegistryCleanupJob, GenericJob, Job, JobExecutor, NativeAsyncJob, PromiseJob,
    },
};
use futures_concurrency::future::FutureGroup;
use smol::{future::FutureExt, stream::StreamExt};
use unsend::{Event, EventListener, EventListenerRc};

use crate::{logger::SharedExternalPrinterLogger, uncaught_job_error};

pub(crate) struct Executor {
    promise_jobs: RefCell<VecDeque<PromiseJob>>,
    async_jobs: RefCell<VecDeque<NativeAsyncJob>>,
    generic_jobs: RefCell<VecDeque<GenericJob>>,
    finalization_registry_jobs: RefCell<VecDeque<FinalizationRegistryCleanupJob>>,
    wake_event: Event<()>,
    idle_event: Event<()>,
    idle_counter: Cell<u8>,

    stop_event: Event<()>,
    printer: SharedExternalPrinterLogger,
}

/// Account for idle tasks even when their waiting future is cancelled.
struct IdleWait<'a>(&'a Cell<u8>);

impl Drop for IdleWait<'_> {
    fn drop(&mut self) {
        self.0.update(|count| count - 1);
    }
}

impl Executor {
    pub(crate) fn new(printer: SharedExternalPrinterLogger) -> Self {
        Self {
            promise_jobs: RefCell::default(),
            async_jobs: RefCell::default(),
            generic_jobs: RefCell::default(),
            finalization_registry_jobs: RefCell::default(),
            wake_event: Event::new(),
            idle_event: Event::new(),
            idle_counter: Cell::new(0),
            stop_event: Event::new(),
            printer,
        }
    }

    pub(crate) fn stop(&self) {
        self.promise_jobs.borrow_mut().clear();
        self.async_jobs.borrow_mut().clear();
        self.generic_jobs.borrow_mut().clear();
        self.finalization_registry_jobs.borrow_mut().clear();
        self.stop_event.notify(u8::MAX);
    }

    /// Waits until there are any new jobs to be handled.
    ///
    /// This will also restore the provided `listener` such that it can keep
    /// listening for more events.
    async fn wait_for_events<'a>(&'a self, mut listener: Pin<&mut EventListener<'a, ()>>) {
        self.idle_event.notify(u8::MAX);

        self.idle_counter.update(|n| n + 1);
        let _idle = IdleWait(&self.idle_counter);
        (&mut listener).await;
        // Restore the listener after usage.
        listener.as_mut().listen();
    }

    /// Continually run all pending promise jobs, yielding to the async
    /// executor after every successful run.
    async fn run_promise_jobs(&self, context: &RefCell<&mut Context>) {
        let mut listener = pin!(EventListener::new(&self.wake_event));
        loop {
            let jobs = mem::take(&mut *self.promise_jobs.borrow_mut());
            if jobs.is_empty() {
                self.wait_for_events(listener.as_mut()).await;
                continue;
            }

            {
                let context = &mut context.borrow_mut();
                for job in jobs {
                    if let Err(e) = job.call(context) {
                        self.printer.print(uncaught_job_error(&e));
                    }
                }
                context.clear_kept_objects();
            }

            smol::future::yield_now().await;
        }
    }

    /// Continually run a single pending generic job, yielding to the async
    /// executor after every successful run.
    async fn run_generic_jobs(&self, context: &RefCell<&mut Context>) {
        let mut listener = pin!(EventListener::new(&self.wake_event));
        loop {
            let job = self.generic_jobs.borrow_mut().pop_front();
            let Some(job) = job else {
                self.wait_for_events(listener.as_mut()).await;
                continue;
            };

            {
                let context = &mut context.borrow_mut();
                if let Err(err) = job.call(context) {
                    self.printer.print(uncaught_job_error(&err));
                }
                context.clear_kept_objects();
            }

            smol::future::yield_now().await;
        }
    }

    /// Continually run all pending async jobs.
    //
    /// This does not need to yield to the async executor after every run because
    /// it assumes that every async job will eventually yield to the executor.
    async fn run_async_jobs(&self, context: &RefCell<&mut Context>) {
        let mut group = FutureGroup::new();
        let mut listener = pin!(EventListener::new(&self.wake_event));
        loop {
            if self.async_jobs.borrow().is_empty() && group.is_empty() {
                self.wait_for_events(listener.as_mut()).await;
            }

            for job in mem::take(&mut *self.async_jobs.borrow_mut()) {
                group.insert(job.call(context));
            }

            let wake = async {
                (&mut listener).await;

                // Restore the listener since it should have been consumed by
                // the await.
                listener.as_mut().listen();
            };

            let next_job = async {
                if let Some(Err(err)) = group.next().await {
                    self.printer.print(uncaught_job_error(&err));
                }
            };

            wake.or(next_job).await;

            context.borrow_mut().clear_kept_objects();
        }
    }

    fn run_ready_cleanup(&self, context: &mut Context) -> bool {
        let job = {
            let mut jobs = self.finalization_registry_jobs.borrow_mut();
            jobs.retain(|job| !job.is_finished());
            let Some(index) = jobs
                .iter()
                .position(FinalizationRegistryCleanupJob::is_ready)
            else {
                return false;
            };
            let job = jobs
                .remove(index)
                .expect("position names an existing cleanup handle");
            jobs.push_back(job.clone());
            job
        };
        if let Err(err) = job.call(context) {
            self.printer.print(uncaught_job_error(&err));
        }
        true
    }

    /// Deliver ready GC notifications at idle without retaining a Context borrow.
    async fn run_finalization_registry_jobs(&self, context: &RefCell<&mut Context>) {
        let mut listener = pin!(EventListener::new(&self.wake_event));
        loop {
            let mut group = FutureGroup::new();
            {
                let mut jobs = self.finalization_registry_jobs.borrow_mut();
                jobs.retain(|job| !job.is_finished());
                for job in jobs.iter().cloned() {
                    group.insert(async move {
                        job.wait_until_ready().await;
                        job
                    });
                }
            }
            let wake = async {
                (&mut listener).await;
                listener.as_mut().listen();
            };
            if group.is_empty() {
                wake.await;
                continue;
            }
            let next = async {
                if let Some(job) = group.next().await
                    && let Err(err) = job.call(&mut context.borrow_mut())
                {
                    self.printer.print(uncaught_job_error(&err));
                }
            };
            wake.or(next).await;
            smol::future::yield_now().await;
        }
    }
}

impl JobExecutor for Executor {
    fn can_enqueue_finalization_registry(&self) -> bool {
        let mut jobs = self.finalization_registry_jobs.borrow_mut();
        jobs.retain(|job| !job.is_finished());
        jobs.len() < 4096
    }

    fn enqueue_job(self: Rc<Self>, job: Job, _context: &mut Context) {
        match job {
            Job::PromiseJob(job) => self.promise_jobs.borrow_mut().push_back(job),
            Job::AsyncJob(job) => self.async_jobs.borrow_mut().push_back(job),
            Job::TimeoutJob(job) => {
                let event = Rc::new(Event::new());
                let listener = EventListenerRc::new(Rc::clone(&event));
                job.cancellation_token().push_callback(move |_| {
                    event.notify(u8::MAX);
                });
                self.async_jobs
                    .borrow_mut()
                    .push_back(NativeAsyncJob::new(async move |context| {
                        // Clamp timeout to prevent setTimeout(fn, 0) loops
                        // from starving the main event loop. 1ms to match Node:
                        // https://nodejs.org/api/timers.html#settimeoutcallback-delay-args
                        const MIN_TIMEOUT: std::time::Duration =
                            std::time::Duration::from_millis(1);
                        let timeout = std::cmp::max(job.timeout().into(), MIN_TIMEOUT);
                        let timer = async {
                            smol::Timer::after(timeout).await;
                            job.call(&mut context.borrow_mut())
                        };
                        let cancel = async {
                            listener.await;
                            Ok(JsValue::undefined())
                        };
                        timer.or(cancel).await
                    }));
            }
            Job::IntervalJob(job) => {
                let event = Rc::new(Event::new());
                let listener = EventListenerRc::new(Rc::clone(&event));
                let printer = self.printer.clone();
                job.cancellation_token().push_callback(move |_| {
                    event.notify(u8::MAX);
                });
                self.async_jobs
                    .borrow_mut()
                    .push_back(NativeAsyncJob::new(async move |context| {
                        let timer = async {
                            let mut interval = smol::Timer::interval(job.interval().into());
                            loop {
                                interval.next().await;
                                if let Err(err) = job.call(&mut context.borrow_mut()) {
                                    printer.print(uncaught_job_error(&err));
                                }
                            }
                        };
                        let cancel = async {
                            listener.await;
                            Ok(JsValue::undefined())
                        };
                        timer.or(cancel).await
                    }));
            }
            Job::GenericJob(job) => self.generic_jobs.borrow_mut().push_back(job),
            Job::FinalizationRegistryCleanupJob(job) => {
                self.finalization_registry_jobs.borrow_mut().push_back(job);
            }
            job => self.printer.print(format!("unsupported job type {job:?}")),
        }
        self.wake_event.notify(u8::MAX);
    }

    fn run_jobs(self: Rc<Self>, context: &mut Context) -> JsResult<()> {
        smol::block_on(self.run_jobs_async(&RefCell::new(context)))
    }

    async fn run_jobs_async(self: Rc<Self>, context: &RefCell<&mut Context>) -> JsResult<()> {
        let executor = smol::LocalExecutor::new();
        let async_task = executor.spawn(self.run_async_jobs(context));
        let generic_task = executor.spawn(self.run_generic_jobs(context));
        let promise_task = executor.spawn(self.run_promise_jobs(context));

        let foreground = async {
            async_task.await;
            generic_task.await;
            promise_task.await;
        };

        let background = async {
            let mut listener = pin!(EventListener::new(&self.idle_event));
            let mut run_fr_jobs = pin!(self.run_finalization_registry_jobs(context));
            let mut cleanup_turns = 0;
            loop {
                let idle_tasks = self.idle_counter.get();
                // Check ready notifications before declaring idle. Yield after
                // each callback so any Promise it enqueued can run first.
                if idle_tasks >= 3 {
                    if cleanup_turns >= 128 || !self.run_ready_cleanup(&mut context.borrow_mut()) {
                        return;
                    }
                    cleanup_turns += 1;
                    smol::future::yield_now().await;
                    continue;
                }

                // Since there are still pending tasks awaiting for IO
                // (probably the async jobs), run any pending finalization registry
                // jobs now that the thread is free to do things.
                //
                // We still need to handle idle event notifications though, because
                // only awaiting the finalization registry jobs would never
                // exit.
                async {
                    (&mut listener).await;

                    // Restore the listener since it should have been consumed by
                    // the await.
                    listener.as_mut().listen();
                }
                .or(&mut run_fr_jobs)
                .await;
            }
        };

        // Stop signal has priority over everything else.
        EventListener::new(&self.stop_event)
            .or(executor.run(foreground))
            .or(background)
            .await;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use boa_engine::Source;

    #[test]
    fn cleanup_runs_at_idle_and_repeated_runs_restore_idle_accounting() {
        let executor = Rc::new(Executor::new(SharedExternalPrinterLogger::new()));
        let mut context = Context::builder()
            .job_executor(executor.clone())
            .build()
            .unwrap();
        context.eval(Source::from_bytes("var values=[]; var registry=new FinalizationRegistry(v=>{values.push(v); Promise.resolve().then(()=>values.push('promise'));}); var target={}; registry.register(target,1);")).unwrap();
        context.run_jobs().unwrap();
        assert_eq!(executor.idle_counter.get(), 0);
        context.eval(Source::from_bytes("target=null")).unwrap();
        boa_gc::force_collect();
        context.run_jobs().unwrap();
        assert_eq!(executor.idle_counter.get(), 0);
        assert_eq!(
            context
                .eval(Source::from_bytes("values.join()==='1,promise'"))
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        context
            .eval(Source::from_bytes(
                "target={}; registry.register(target,2);",
            ))
            .unwrap();
        context.run_jobs().unwrap();
        context.eval(Source::from_bytes("target=null")).unwrap();
        boa_gc::force_collect();
        context.run_jobs().unwrap();
        assert_eq!(
            context
                .eval(Source::from_bytes("values.join()==='1,promise,2,promise'"))
                .unwrap()
                .as_boolean(),
            Some(true)
        );
        assert_eq!(executor.idle_counter.get(), 0);
    }
}
