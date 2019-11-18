use super::{Config, GlobalQueue, PoolContext, SchedUnit};
use crossbeam_deque::Steal;
use std::marker::PhantomData;

pub enum Task<G>
where
    G: GlobalQueue,
{
    Once(Option<Box<dyn FnOnce(&mut Handle<'_, G>) + Send>>),
    Mut(Box<dyn FnMut(&mut Handle<'_, G>) + Send>),
}

impl<G> AsMut<Self> for Task<G>
where
    G: GlobalQueue,
{
    fn as_mut(&mut self) -> &mut Self {
        self
    }
}

pub struct Runner<G>
where
    G: GlobalQueue,
{
    max_inplace_spin: usize,
    _phantom: PhantomData<G>,
}

impl<G, T> super::Runner for Runner<G>
where
    G: GlobalQueue<Task = T>,
    T: AsMut<Task<G>>,
{
    type GlobalQueue = G;

    fn handle(&mut self, ctx: &mut PoolContext<G>, mut task: G::Task) -> bool {
        let mut handle = Handle { ctx, rerun: false };
        match task.as_mut() {
            Task::Mut(ref mut r) => {
                let mut tried_times = 0;
                loop {
                    r(&mut handle);
                    if !handle.rerun {
                        return true;
                    }
                    // TODO: fix the bug here when set to true.
                    handle.rerun = false;
                    tried_times += 1;
                    if tried_times == self.max_inplace_spin {
                        break;
                    }
                }
            }
            Task::Once(ref mut r) => {
                (r.take().unwrap())(&mut handle);
                return true;
            }
        }
        ctx.spawn(task);
        false
    }
}

pub struct Handle<'a, G>
where
    G: GlobalQueue,
{
    ctx: &'a mut PoolContext<G>,
    rerun: bool,
}

impl<'a, G> Handle<'a, G>
where
    G: GlobalQueue<Task = Task<G>>,
{
    pub fn spawn_once(&mut self, t: impl FnOnce(&mut Handle<'_, G>) + Send + 'static) {
        self.ctx.spawn(Task::Once(Some(Box::new(t))));
    }

    pub fn spawn_mut(&mut self, t: impl FnMut(&mut Handle<'_, G>) + Send + 'static) {
        self.ctx.spawn(Task::Mut(Box::new(t)));
    }

    pub fn rerun(&mut self) {
        self.rerun = true;
    }

    pub fn to_owned(&self) -> Remote<G> {
        Remote {
            remote: self.ctx.remote(),
        }
    }
}

pub struct Remote<G>
where
    G: GlobalQueue,
{
    remote: super::Remote<G>,
}

impl<G> Remote<G>
where
    G: GlobalQueue<Task = Task<G>>,
{
    pub fn spawn_once(&self, t: impl FnOnce(&mut Handle<'_, G>) + Send + 'static) {
        self.remote.spawn(Task::Once(Some(Box::new(t))));
    }

    pub fn spawn_mut(&self, t: impl FnMut(&mut Handle<'_, G>) + Send + 'static) {
        self.remote.spawn(Task::Mut(Box::new(t)))
    }
}

pub struct RunnerFactory<G>
where
    G: GlobalQueue,
{
    max_inplace_spin: usize,
    _phantom: PhantomData<G>,
}

impl<G> RunnerFactory<G>
where
    G: GlobalQueue,
{
    pub fn new() -> Self {
        RunnerFactory {
            max_inplace_spin: 4,
            _phantom: PhantomData,
        }
    }

    pub fn set_max_inplace_spin(&mut self, count: usize) {
        self.max_inplace_spin = count;
    }
}

impl<G, T> super::RunnerFactory for RunnerFactory<G>
where
    G: GlobalQueue<Task = T>,
    T: AsMut<Task<G>>,
{
    type Runner = Runner<G>;

    fn produce(&mut self) -> Runner<G> {
        Runner {
            max_inplace_spin: self.max_inplace_spin,
            _phantom: PhantomData,
        }
    }
}

impl<G> super::ThreadPool<G>
where
    G: GlobalQueue<Task = Task<G>>,
{
    pub fn spawn_once(&self, t: impl FnOnce(&mut Handle<'_, G>) + Send + 'static) {
        self.spawn(Task::Once(Some(Box::new(t))));
    }

    pub fn spawn_mut(&self, t: impl FnMut(&mut Handle<'_, G>) + Send + 'static) {
        self.spawn(Task::Mut(Box::new(t)))
    }
}

// For lack of lazy normalization, a wrapper type is needed to avoid cyclic type error.

pub struct SingleQueue(crossbeam_deque::Injector<SchedUnit<Task<SingleQueue>>>);

impl GlobalQueue for SingleQueue {
    type Task = Task<SingleQueue>;

    fn steal_batch_and_pop(
        &self,
        local_queue: &crossbeam_deque::Worker<SchedUnit<Self::Task>>,
    ) -> Steal<SchedUnit<Self::Task>> {
        crossbeam_deque::Injector::steal_batch_and_pop(&self.0, local_queue)
    }
    fn push(&self, task: SchedUnit<Self::Task>) {
        self.0.push(task);
    }
}

pub struct SimpleThreadPool(super::ThreadPool<SingleQueue>);

impl SimpleThreadPool {
    pub fn from_config(config: Config) -> Self {
        let pool = config.spawn(RunnerFactory::new(), || {
            SingleQueue(crossbeam_deque::Injector::new())
        });
        Self(pool)
    }

    pub fn spawn_once(&self, t: impl FnOnce(&mut Handle<'_, SingleQueue>) + Send + 'static) {
        self.0.spawn_once(t)
    }

    pub fn spawn_mut(&self, t: impl FnMut(&mut Handle<'_, SingleQueue>) + Send + 'static) {
        self.0.spawn_mut(t)
    }
}
